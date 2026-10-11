import assert from 'node:assert/strict'
import test from 'node:test'
import { buildMainEmptySuggestionAction, splitSidebarGroups } from './sidebarGroups.model.ts'

test('splits project chats into active, completed, and archived sections', () => {
  const chats = [
    { id: 'active', kind: 'project', project_id: 'p-active' },
    { id: 'review', kind: 'project', project_id: 'p-review' },
    { id: 'done', kind: 'project', project_id: 'p-done' },
    { id: 'archived', kind: 'project', project_id: 'p-archived' },
    { id: 'other', kind: 'direct', bot_id: 'b1' },
  ]
  const projects = {
    'p-active': { id: 'p-active', chat_id: 'active', status: 'active' },
    'p-review': { id: 'p-review', chat_id: 'review', status: 'review' },
    'p-done': { id: 'p-done', chat_id: 'done', status: 'done' },
    'p-archived': { id: 'p-archived', chat_id: 'archived', status: 'archived' },
  }

  assert.deepEqual(splitSidebarGroups(chats, projects), {
    active: [chats[0], chats[1]],
    done: [chats[2]],
    archived: [chats[3]],
  })
})

test('keeps a main empty suggestion tied to the main chat and preserves its draft', () => {
  assert.deepEqual(buildMainEmptySuggestionAction('帮我给 App 加个登录', 'main-chat'), {
    chatId: 'main-chat',
    draft: '帮我给 App 加个登录',
  })
  assert.deepEqual(buildMainEmptySuggestionAction('调研 3 个竞品', null), {
    chatId: null,
    draft: '调研 3 个竞品',
  })
})
