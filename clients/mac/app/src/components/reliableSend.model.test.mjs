import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import vm from 'node:vm'
import { URL } from 'node:url'
import { transform } from 'esbuild'
import { prepareSendAttempt, acceptBridgeSnapshot } from './reliableSend.model.ts'

const source = fs.readFileSync(new URL('../App.tsx', import.meta.url), 'utf8')
const body = source.slice(source.indexOf('  async function sendMessage('), source.indexOf('  const hostStartup =', source.indexOf('  async function sendMessage(')))
const compiled = (await transform(body, { loader: 'ts' })).code
for (const variant of ['text', 'mention', 'quote', 'attachment', 'skill', 'skill-only']) {
  test(`production sendMessage durably queues ${variant} and reuses the operation ID after an unknown result`, async () => {
    const calls = []
    const noop = () => {}
    let ids = 0
    const environment = {
      currentChat: { id: 'chat-a' }, attachments: variant === 'attachment' ? [{ status: 'ready', uploadId: 'upload-a' }] : [],
      composerSkill: variant.startsWith('skill') ? { name: 'test' } : null, draft: variant === 'skill-only' ? '' : 'hello', sending: false,
      editingMessage: null,
      state: { connection: 'connected' }, quoteTarget: variant === 'quote' ? { id: 'quoted' } : null,
      mentionBotId: variant === 'mention' ? 'bot-a' : null, hostScope: 'host-a', hostScopeRef: { current: 'host-a' },
      selectedChatRef: { current: 'chat-a' }, sendAttemptRef: { current: null }, sendingOperationRef: { current: 0 }, prepareSendAttempt,
      crypto: { randomUUID: () => `operation-${++ids}` }, quoteSendFields: () => variant === 'quote' ? { quote: 'quoted', quote_mode: 'reference' } : {},
      chatSendMentions: () => variant === 'mention' ? [{ kind: 'bot', bot_id: 'bot-a' }] : [],
      invoke: async (command, args) => { calls.push({ command, args }); throw new Error('request timed out') },
      assertHostScope: noop, setSending: noop, setSendError: noop, setDraftForChat: noop,
      setMentionBotId: noop, setComposerSkill: noop, setSlashPicker: noop, setSlashSelectedIndex: noop,
      setQuoteTarget: noop, setAttachments: noop,
    }
    const send = vm.runInNewContext(`${compiled}; sendMessage`, environment)
    await send({ preventDefault: noop })
    await send({ preventDefault: noop })
    assert.equal(calls.length, 2)
    assert.equal(calls[0].command, 'send_message')
    assert.equal(calls[0].args.hostId, 'host-a')
    assert.ok(calls[0].args.params.client_request_id)
    assert.equal(calls[0].args.params.client_request_id, calls[1].args.params.client_request_id)
    assert.deepEqual(JSON.parse(JSON.stringify(calls[0].args.params.mentions)), environment.chatSendMentions())
    if (variant === 'attachment') assert.deepEqual(JSON.parse(JSON.stringify(calls[0].args.params.attachments)), ['upload-a'])
    if (variant === 'quote') assert.equal(calls[0].args.params.quote, 'quoted')
    if (variant === 'skill') assert.equal(calls[0].args.params.text, '/test hello')
    if (variant === 'skill-only') assert.equal(calls[0].args.params.text, '/test')
  })
}

test('production sendMessage edits the selected message through message.edit and clears the edit draft on success', async () => {
  const calls = []
  const drafts = []
  const editedMessages = []
  const sendingStates = []
  const environment = {
    editingMessage: { hostScope: 'host-a', chatId: 'chat-a', messageId: 'message-7' },
    draft: '  corrected text  ', currentChat: { id: 'chat-a' }, hostScope: 'host-a', hostScopeRef: { current: 'host-a' },
    state: { connection: 'connected' }, sending: false, sendingOperationRef: { current: 0 }, selectedChatRef: { current: 'chat-a' },
    invoke: async (command, args) => { calls.push({ command, args }); return { message: { id: 'message-7', text: 'corrected text' } } },
    assertHostScope: () => {}, updateMessageFromAction: (...args) => editedMessages.push(args),
    setDraftForChat: (...args) => drafts.push(args), setEditingMessage: (value) => { environment.editingMessage = value },
    setSending: (value) => sendingStates.push(value), setSendError: () => {},
  }
  const send = vm.runInNewContext(`${compiled}; sendMessage`, environment)
  await send({ preventDefault() {} })
  assert.deepEqual(calls.map(({ command }) => command), ['protocol_request'])
  assert.equal(calls[0].args.method, 'message.edit')
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].args.params)), { message_id: 'message-7', text: 'corrected text' })
  assert.equal(editedMessages.length, 1)
  assert.deepEqual(drafts, [['chat-a', '']])
  assert.equal(environment.editingMessage, null)
  assert.deepEqual(sendingStates, [true, false])
})

test('production sendMessage preserves the edit target and draft when message.edit fails', async () => {
  const draft = 'revised text'
  const edit = { hostScope: 'host-a', chatId: 'chat-a', messageId: 'message-7' }
  let error = null
  const environment = {
    editingMessage: edit, draft, currentChat: { id: 'chat-a' }, hostScope: 'host-a', hostScopeRef: { current: 'host-a' },
    state: { connection: 'connected' }, sending: false, sendingOperationRef: { current: 0 }, selectedChatRef: { current: 'chat-a' },
    invoke: async () => { throw new Error('edit rejected') }, assertHostScope: () => {}, updateMessageFromAction: () => {},
    setDraftForChat: () => assert.fail('failed edit must preserve draft'), setEditingMessage: () => assert.fail('failed edit must remain active'),
    setSending: () => {}, setSendError: (value) => { error = value },
  }
  const send = vm.runInNewContext(`${compiled}; sendMessage`, environment)
  await send({ preventDefault() {} })
  assert.equal(environment.editingMessage, edit)
  assert.equal(environment.draft, draft)
  assert.match(error, /edit rejected/)
})

test('changed content and another Host get independent operation IDs', () => {
  const first = prepareSendAttempt(null, 'a', { text: 'one' }, () => 'id-1')
  assert.equal(prepareSendAttempt(first, 'a', { text: 'one' }, () => 'unused'), first)
  assert.equal(prepareSendAttempt(first, 'b', { text: 'one' }, () => 'id-2').params.client_request_id, 'id-2')
  assert.equal(prepareSendAttempt(first, 'a', { text: 'two' }, () => 'id-3').params.client_request_id, 'id-3')
})

test('a late get_bridge_state cannot replace newer Host/events/outbox state', () => {
  const latest = { revision: 8, host: 'b', pending: [] }
  assert.equal(acceptBridgeSnapshot(latest, { revision: 7, host: 'a', pending: ['old'] }), latest)
  assert.equal(acceptBridgeSnapshot(latest, { revision: 9, host: 'b', pending: ['new'] }).revision, 9)
})
