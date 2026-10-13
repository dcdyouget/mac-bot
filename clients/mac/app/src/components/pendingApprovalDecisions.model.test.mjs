import assert from 'node:assert/strict'
import test from 'node:test'
import { pendingApprovalsForContext, pendingDecisionsForContext } from './pendingApprovalDecisions.model.ts'

const approvals = {
  main: { id: 'main', state: 'pending', bot_id: 'bot-main', chat_id: 'chat-main', summary: '主 Bot 操作', created_at: '2026-01-03T00:00:00Z' },
  worker: { id: 'worker', state: 'pending', bot_id: 'bot-worker', chat_id: 'chat-worker', assignment_id: 'assignment-project', summary: '群内 Bot 操作', created_at: '2026-01-02T00:00:00Z' },
  unrelated: { id: 'unrelated', state: 'pending', bot_id: 'bot-other', chat_id: 'chat-other', summary: '无关操作', created_at: '2026-01-01T00:00:00Z' },
  resolved: { id: 'resolved', state: 'allowed_once', bot_id: 'bot-worker', chat_id: 'chat-worker', summary: '已处理', created_at: '2026-01-04T00:00:00Z' },
}

test('main chat acts as the global pending-decision inbox, newest first', () => {
  const result = pendingApprovalsForContext(approvals, { chatId: 'chat-main', isMain: true })
  assert.deepEqual(result.map(({ id }) => id), ['main', 'worker', 'unrelated'])
})

test('Bot chat shows all pending approvals owned by that Bot', () => {
  const result = pendingApprovalsForContext(approvals, { chatId: 'chat-worker-dm', botId: 'bot-worker' })
  assert.deepEqual(result.map(({ id }) => id), ['worker'])
})

test('group chat includes approvals associated through assignment project or an approval reference', () => {
  const result = pendingApprovalsForContext(
    approvals,
    { chatId: 'chat-project', projectId: 'project-1' },
    { 'assignment-project': { project_id: 'project-1' } },
    new Set(['unrelated']),
  )
  assert.deepEqual(result.map(({ id }) => id), ['worker', 'unrelated'])
  assert.equal(result[0].chatId, 'chat-worker')
})

test('direct chat includes its own pending approvals even when legacy records lack bot_id', () => {
  const result = pendingApprovalsForContext({ legacy: { state: 'pending', chat_id: 'chat-current' } }, { chatId: 'chat-current', botId: 'bot-current' })
  assert.deepEqual(result.map(({ id }) => id), ['legacy'])
})

test('main decision list includes all pending questions and project reviews', () => {
  const result = pendingDecisionsForContext(
    {},
    {
      question1: { id: 'question1', state: 'pending', chat_id: 'chat-worker', bot_id: 'bot-worker', text: '要选哪种方案？' },
      question2: { id: 'question2', state: 'answered', chat_id: 'chat-main', bot_id: 'bot-main', text: '已经回答' },
    },
    {
      project1: { id: 'project1', chat_id: 'chat-project', name: '待验收项目', status: 'review' },
      project2: { id: 'project2', chat_id: 'chat-other', name: '进行中项目', status: 'active' },
    },
    [],
    { chatId: 'chat-main', isMain: true },
  )
  assert.deepEqual(result.map(({ kind, id, chatId }) => ({ kind, id, chatId })), [
    { kind: 'question', id: 'question1', chatId: 'chat-worker' },
    { kind: 'review', id: 'project1', chatId: 'chat-project' },
  ])
})

test('Bot decision list includes its pending questions and reviews in projects it belongs to', () => {
  const result = pendingDecisionsForContext(
    {},
    {
      owned: { id: 'owned', state: 'pending', chat_id: 'chat-worker', bot_id: 'bot-worker', text: '需要你选方案' },
      other: { id: 'other', state: 'pending', chat_id: 'chat-other', bot_id: 'bot-other', text: '其他 Bot 的问题' },
    },
    {
      assigned: { id: 'assigned', chat_id: 'chat-project', name: 'Bot 参与的项目', status: 'review', members: [{ bot_id: 'bot-worker' }] },
      unrelated: { id: 'unrelated', chat_id: 'chat-other', name: '无关项目', status: 'review', members: [{ bot_id: 'bot-other' }] },
    },
    [],
    { chatId: 'chat-worker', botId: 'bot-worker' },
  )
  assert.deepEqual(result.map(({ kind, id, chatId }) => ({ kind, id, chatId })), [
    { kind: 'question', id: 'owned', chatId: 'chat-worker' },
    { kind: 'review', id: 'assigned', chatId: 'chat-project' },
  ])
})

test('group decision list is restricted to that project and resolves its chat target', () => {
  const result = pendingDecisionsForContext(
    {},
    { question: { state: 'pending', chat_id: 'chat-worker', assignment_id: 'assignment', text: '请确认' } },
    { project: { id: 'project', chat_id: 'chat-project', name: '目标群', status: 'review' }, other: { id: 'other', chat_id: 'chat-other', status: 'review' } },
    [{ id: 'chat-project', project_id: 'project' }],
    { chatId: 'chat-project', projectId: 'project' },
    { assignment: { project_id: 'project' } },
  )
  assert.deepEqual(result.map(({ kind, id, chatId }) => ({ kind, id, chatId })), [
    { kind: 'question', id: 'question', chatId: 'chat-worker' },
    { kind: 'review', id: 'project', chatId: 'chat-project' },
  ])
})
