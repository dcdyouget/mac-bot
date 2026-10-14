import test from 'node:test'
import assert from 'node:assert/strict'
import { chatDraftKey, groupProductMessage, mergeChatMessages, orderedGroupMemberIds, projectAssignmentForMember, setChatDraft } from './groupConversation.model.mjs'

test('composer drafts are isolated by both host and chat', () => {
  const first = setChatDraft({}, 'host-a', 'group-1', 'draft one')
  const second = setChatDraft(first, 'host-a', 'group-2', 'draft two')
  const third = setChatDraft(second, 'host-b', 'group-1', 'other host')
  assert.equal(third[chatDraftKey('host-a', 'group-1')], 'draft one')
  assert.equal(third[chatDraftKey('host-a', 'group-2')], 'draft two')
  assert.equal(third[chatDraftKey('host-b', 'group-1')], 'other host')
})

test('message IDs are deduplicated per chat and newer authoritative values win', () => {
  const history = { id: 'same', chat_id: 'a', seq: 2, fallback_text: 'old' }
  const pendingCollision = { id: 'same', chat_id: 'b', seq: 1, fallback_text: 'other chat' }
  const canonical = { ...history, fallback_text: 'edited' }
  const result = mergeChatMessages([history, pendingCollision], [canonical])
  assert.equal(result.length, 2)
  assert.equal(result.find((message) => message.chat_id === 'a').fallback_text, 'edited')
  assert.equal(result.find((message) => message.chat_id === 'b').fallback_text, 'other chat')
})

test('messages sort by sequence within a chat then remain stable across ID collisions', () => {
  const result = mergeChatMessages([
    { id: 'later', chat_id: 'a', seq: 9, created_at: '2026-01-02' },
    { id: 'earlier', chat_id: 'a', seq: 2, created_at: '2026-01-01' },
    { id: 'earlier', chat_id: 'b', seq: 1, created_at: '2026-01-01' },
  ])
  assert.deepEqual(result.map((message) => `${message.chat_id}:${message.id}`), ['a:earlier', 'b:earlier', 'a:later'])
})

test('group avatar member order puts the main Bot first and otherwise follows join order', () => {
  assert.deepEqual(orderedGroupMemberIds({
    projectMembers: [{ bot_id: 'late', joined_at: '2026-03' }, { bot_id: 'main', joined_at: '2026-02' }, { bot_id: 'early', joined_at: '2026-01' }],
    memberBotIds: ['extra', 'late'],
    bots: { main: { is_main: true } },
  }), ['main', 'early', 'late', 'extra'])
})

test('group member assignment ignores a preferred assignment from another project', () => {
  const assignments = {
    old: { id: 'old', project_id: 'project-old', origin_chat_id: 'chat-old', bot_id: 'coder', status: 'done' },
    current: { id: 'current', project_id: 'project-current', origin_chat_id: 'chat-current', bot_id: 'coder', status: 'working' },
  }
  assert.equal(projectAssignmentForMember('project-current', 'chat-current', 'coder', 'old', assignments), assignments.current)
})

test('group member assignment ignores a preferred assignment for another Bot in the same project', () => {
  const assignments = {
    otherBot: { id: 'otherBot', project_id: 'project-current', origin_chat_id: 'chat-current', bot_id: 'reviewer', status: 'blocked' },
    currentBot: { id: 'currentBot', project_id: 'project-current', origin_chat_id: 'chat-current', bot_id: 'coder', status: 'working' },
  }
  assert.equal(projectAssignmentForMember('project-current', 'chat-current', 'coder', 'otherBot', assignments), assignments.currentBot)
})

test('group member assignment does not leak a stale preferred ID when there is no scoped assignment', () => {
  const assignments = { old: { id: 'old', project_id: 'project-old', origin_chat_id: 'chat-old', bot_id: 'coder', status: 'working' } }
  assert.equal(projectAssignmentForMember('project-current', 'chat-current', 'coder', 'old', assignments), undefined)
  assert.equal(projectAssignmentForMember('', 'chat-current', 'coder', 'old', assignments), undefined)
  assert.equal(projectAssignmentForMember('project-current', '', 'coder', 'old', assignments), undefined)
})

test('group member assignment trusts the preferred ID only when group, project, and Bot all match', () => {
  const assignments = {
    preferred: { id: 'preferred', project_id: 'project-current', origin_chat_id: 'chat-current', bot_id: 'coder', status: 'done' },
    newer: { id: 'newer', project_id: 'project-current', origin_chat_id: 'chat-current', bot_id: 'coder', status: 'working' },
  }
  assert.equal(projectAssignmentForMember('project-current', 'chat-current', 'coder', 'preferred', assignments), assignments.preferred)
})

test('group member assignment does not select an assignment from another group chat in the same project', () => {
  const assignments = {
    sibling: { id: 'sibling', project_id: 'project-current', origin_chat_id: 'chat-sibling', bot_id: 'coder', status: 'blocked' },
    current: { id: 'current', project_id: 'project-current', origin_chat_id: 'chat-current', bot_id: 'coder', status: 'working' },
  }
  assert.equal(projectAssignmentForMember('project-current', 'chat-current', 'coder', 'sibling', assignments), assignments.current)
})

test('group chat keeps product nodes, suppresses progress, and uses fallback text for unknown blocks', () => {
  assert.equal(groupProductMessage({ sender: { kind: 'bot' }, intent: 'progress', blocks: [{ type: 'progress' }] }), null)
  const ack = groupProductMessage({ sender: { kind: 'bot' }, intent: 'ack', blocks: [{ type: 'text', markdown: '收到' }] })
  assert.equal(ack.blocks[0].markdown, '收到')
  const unknown = groupProductMessage({ sender: { kind: 'bot' }, intent: 'done', blocks: [{ type: 'future_block', opaque: true }], fallback_text: '已完成' })
  assert.deepEqual(unknown.blocks, [{ type: 'text', markdown: '已完成' }])
  assert.equal(groupProductMessage({ sender: { kind: 'bot' }, intent: 'done', blocks: [{ type: 'completion' }] }).blocks[0].type, 'completion')
})
