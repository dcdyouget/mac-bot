import assert from 'node:assert/strict'
import test from 'node:test'
import {
  appendMessageQuoteToAnnouncement,
  chatSendMentions,
  isMessageMenuActionCurrent,
  messageDeepLink,
  messageAssignmentId,
  messageRenderMode,
  messageShortcutAction,
  quoteSendFields,
  shouldSendOnCommandEnter,
  userHasReaction,
} from './messageActions.model.ts'

test('message keyboard actions ignore editable and composing focus', () => {
  assert.equal(messageShortcutAction({ key: 'r' }), 'reply')
  assert.equal(messageShortcutAction({ key: 'q' }), 'reference')
  assert.equal(messageShortcutAction({ key: 'c', metaKey: true }), 'copy')
  assert.equal(messageShortcutAction({ key: 'r', editable: true }), null)
  assert.equal(messageShortcutAction({ key: 'q', isComposing: true }), null)
  assert.equal(messageShortcutAction({ key: 'r', shiftKey: true }), null)
})

test('Command+Enter submits the composer without intercepting newlines or IME confirmation', () => {
  assert.equal(shouldSendOnCommandEnter({ key: 'Enter', metaKey: true }), true)
  assert.equal(shouldSendOnCommandEnter({ key: 'Enter', ctrlKey: true }), true)
  assert.equal(shouldSendOnCommandEnter({ key: 'Enter', metaKey: true, shiftKey: true }), false)
  assert.equal(shouldSendOnCommandEnter({ key: 'Enter', metaKey: true, altKey: true }), false)
  assert.equal(shouldSendOnCommandEnter({ key: 'Enter', metaKey: true, isComposing: true }), false)
  assert.equal(shouldSendOnCommandEnter({ key: 'Enter', metaKey: true, keyCode: 229 }), false)
  assert.equal(shouldSendOnCommandEnter({ key: 'Enter' }), false)
})

test('streaming messages use a placeholder until final content arrives', () => {
  assert.equal(messageRenderMode({ streaming: true }), 'streaming')
  assert.equal(messageRenderMode({ streaming: false }), 'content')
  assert.equal(messageRenderMode({ streaming: true, deleted: true }), 'deleted')
})

test('reply quotes target bot while references never add an automatic mention', () => {
  assert.deepEqual(quoteSendFields({ messageId: 'm1', mode: 'reply', botId: 'bot-1' }), {
    quote: 'm1', quote_mode: 'reply', mentions: [{ kind: 'bot', bot_id: 'bot-1', instruction: null }],
  })
  assert.deepEqual(quoteSendFields({ messageId: 'm2', mode: 'reference', botId: 'bot-1' }), {
    quote: 'm2', quote_mode: 'reference', mentions: [],
  })
  assert.deepEqual(chatSendMentions(null, { messageId: 'm1', mode: 'reply', botId: 'bot-1' }), [
    { kind: 'bot', bot_id: 'bot-1', instruction: null },
  ])
  assert.deepEqual(chatSendMentions('bot-2', { messageId: 'm1', mode: 'reply', botId: 'bot-1' }).map((mention) => mention.bot_id), ['bot-2', 'bot-1'])
  assert.deepEqual(chatSendMentions(null, { messageId: 'm2', mode: 'reference', botId: 'bot-1' }), [])
})

test('announcement quote appends Markdown without replacing existing text', () => {
  assert.equal(appendMessageQuoteToAnnouncement('Current plan', '编码', 'Do this\nThen that'), 'Current plan\n\n> **编码**\n> Do this\n> Then that')
  assert.equal(appendMessageQuoteToAnnouncement('', '用户', ''), '> **用户**')
})

test('message links encode opaque identifiers and reaction state detects user sender', () => {
  assert.equal(messageDeepLink('task:attention/1', 'chat-1', 'host-1'), 'macbot://message/task%3Aattention%2F1?chat_id=chat-1&host_id=host-1')
  assert.equal(userHasReaction({ by: [{ kind: 'bot' }, { kind: 'user' }] }), true)
  assert.equal(userHasReaction({ by: [{ kind: 'bot' }] }), false)
  assert.equal(messageAssignmentId({ assignment_id: null, blocks: [{ assignment_id: 'assign-1' }] }), 'assign-1')
  assert.equal(messageAssignmentId({ assignment_id: 'assign-2', blocks: [{ assignment_id: 'assign-1' }] }), 'assign-2')
})

test('message menu async actions are scoped to the original host and menu instance', () => {
  const originalMenu = { message: { id: 'message-1' }, x: 20, y: 30 }
  const reopenedMenu = { message: { id: 'message-1' }, x: 40, y: 50 }

  assert.equal(isMessageMenuActionCurrent('host-a', 'host-a', originalMenu, originalMenu), true)
  assert.equal(isMessageMenuActionCurrent('host-b', 'host-a', originalMenu, originalMenu), false)
  assert.equal(isMessageMenuActionCurrent('host-a', 'host-a', null, originalMenu), false)
  assert.equal(isMessageMenuActionCurrent('host-a', 'host-a', reopenedMenu, originalMenu), false)
})
