import assert from 'node:assert/strict'
import test from 'node:test'
import { classifyDeepLink } from './deepLinkRouting.model.ts'

const valid = { status: 'valid', link: { message_id: 'm1', chat_id: 'c1', host_id: 'host-a' } }

test('rejects invalid or incomplete native link results', () => {
  assert.equal(classifyDeepLink({ status: 'invalid' }, true, 'host-a', { c1: {} }), 'invalid')
  assert.equal(classifyDeepLink({ status: 'valid', link: { ...valid.link, message_id: '' } }, true, 'host-a', { c1: {} }), 'invalid')
})

test('waits for a connected Host before checking its identity', () => {
  assert.equal(classifyDeepLink(valid, false, null, {}), 'wait_for_host')
  assert.equal(classifyDeepLink(valid, true, null, {}), 'wait_for_host')
})

test('requires the active server node and target chat to match before navigation', () => {
  assert.equal(classifyDeepLink(valid, true, 'host-b', { c1: {} }), 'host_mismatch')
  assert.equal(classifyDeepLink(valid, true, 'host-a', {}), 'chat_missing')
  assert.equal(classifyDeepLink(valid, true, 'host-a', { c1: {} }), 'ready')
})
